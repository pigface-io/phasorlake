-- ************************************************************
-- Title:	PUBLIC_DVD_STPASA_CONSTRAINTSOLUTION_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS STPASA_CONSTRAINTSOLUTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_STPASA_CONSTRAINTSOLUTION_202401010000.CSV
APPEND INTO TABLE STPASA_CONSTRAINTSOLUTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
RUN_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
INTERVAL_DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
CONSTRAINTID CHAR(20),
CAPACITYRHS FLOAT EXTERNAL,
CAPACITYMARGINALVALUE FLOAT EXTERNAL,
CAPACITYVIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
RUNTYPE CHAR(20),
STUDYREGIONID CHAR(20))
-- End of Script
