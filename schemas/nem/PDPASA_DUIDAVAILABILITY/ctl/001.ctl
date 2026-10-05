-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PDPASA_DUIDAVAILABILITY#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PDPASA_DUIDAVAILABILITY owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PDPASA_DUIDAVAILABILITY#FILE01#202608010000.CSV
APPEND INTO TABLE PDPASA_DUIDAVAILABILITY
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
DUID CHAR(20),
GENERATION_MAX_AVAILABILITY FLOAT EXTERNAL,
GENERATION_PASA_AVAILABILITY FLOAT EXTERNAL,
GENERATION_RECALL_PERIOD FLOAT EXTERNAL,
LOAD_MAX_AVAILABILITY FLOAT EXTERNAL,
LOAD_PASA_AVAILABILITY FLOAT EXTERNAL,
LOAD_RECALL_PERIOD FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
