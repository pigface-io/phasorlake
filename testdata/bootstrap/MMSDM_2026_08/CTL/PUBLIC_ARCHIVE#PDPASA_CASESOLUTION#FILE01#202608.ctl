-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PDPASA_CASESOLUTION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PDPASA_CASESOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PDPASA_CASESOLUTION#FILE01#202608010000.CSV
APPEND INTO TABLE PDPASA_CASESOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
PASAVERSION CHAR(10),
RESERVECONDITION FLOAT EXTERNAL,
LORCONDITION FLOAT EXTERNAL,
CAPACITYOBJFUNCTION FLOAT EXTERNAL,
CAPACITYOPTION FLOAT EXTERNAL,
MAXSURPLUSRESERVEOPTION FLOAT EXTERNAL,
MAXSPARECAPACITYOPTION FLOAT EXTERNAL,
INTERCONNECTORFLOWPENALTY FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
RELIABILITYLRCDEMANDOPTION FLOAT EXTERNAL,
OUTAGELRCDEMANDOPTION FLOAT EXTERNAL,
LORDEMANDOPTION FLOAT EXTERNAL,
RELIABILITYLRCCAPACITYOPTION CHAR(10),
OUTAGELRCCAPACITYOPTION CHAR(10),
LORCAPACITYOPTION CHAR(10),
LORUIGFOPTION FLOAT EXTERNAL,
RELIABILITYLRCUIGFOPTION FLOAT EXTERNAL,
OUTAGELRCUIGFOPTION FLOAT EXTERNAL)
-- End of Script
