-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PDPASA_INTERCONNECTORSOLN#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PDPASA_INTERCONNECTORSOLN owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PDPASA_INTERCONNECTORSOLN#FILE01#202608010000.CSV
APPEND INTO TABLE PDPASA_INTERCONNECTORSOLN
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
INTERVAL_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
INTERCONNECTORID CHAR(10),
CAPACITYMWFLOW FLOAT EXTERNAL,
CAPACITYMARGINALVALUE FLOAT EXTERNAL,
CAPACITYVIOLATIONDEGREE FLOAT EXTERNAL,
CALCULATEDEXPORTLIMIT FLOAT EXTERNAL,
CALCULATEDIMPORTLIMIT FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
RUNTYPE CHAR(20),
EXPORTLIMITCONSTRAINTID CHAR(20),
IMPORTLIMITCONSTRAINTID CHAR(20),
STUDYREGIONID CHAR(20))
-- End of Script
