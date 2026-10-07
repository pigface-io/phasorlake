-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#GENUNITS_UNIT#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENUNITS_UNIT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#GENUNITS_UNIT#FILE01#202408010000.CSV
APPEND INTO TABLE GENUNITS_UNIT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
GENSETID CHAR(20),
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
UNIT_GROUPING_LABEL CHAR(20),
UNIT_COUNT FLOAT EXTERNAL,
UNIT_SIZE FLOAT EXTERNAL,
UNIT_MAX_SIZE FLOAT EXTERNAL,
AGGREGATION_FLAG FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
UNITMINSIZE FLOAT EXTERNAL,
MAXSTORAGECAPACITY FLOAT EXTERNAL,
REGISTEREDCAPACITY FLOAT EXTERNAL,
REGISTEREDMINCAPACITY FLOAT EXTERNAL)
-- End of Script
